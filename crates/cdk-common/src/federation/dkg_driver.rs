//! Pure, restartable planning for a dealer-free federation DKG ceremony.

use core::fmt;
use std::collections::BTreeMap;

use super::{
    FederationConfig, FederationDkgCommitmentRequest, FederationDkgFinalizedMemberConfig,
    FederationDkgMemberKeysetShare, FederationDkgParticipantCommitment,
    FederationDkgParticipantCommitmentReveal, FederationDkgParticipantState,
    FederationDkgRevealRequest, FederationDkgSecretShareRequest, FederationDkgSetupMethod,
    FederationDkgSetupState, FederationDkgTranscript, FederationDkgTranscriptSignature,
    FederationDkgTranscriptSignatureRequest, FederationError, MemberConfigPrivate, MemberId,
    Result,
};

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
    /// Every member signature is present and final config assembly succeeds.
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
        f.debug_struct("FederationDkgDriverPlan")
            .field("phase", &self.phase)
            .field("progress", &self.progress)
            .field("has_commitment", &self.commitment.is_some())
            .field("has_reveal", &self.reveal.is_some())
            .field("secret_share_receivers", &secret_share_receivers)
            .field("derived_keyset_id", &derived_keyset_id)
            .field("transcript_hash", &transcript_hash)
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
        public_config.validate()?;
        private_config.validate_against_public(public_config)?;
        participant_state.validate(public_config)?;
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
            ));
        }

        let round = setup_state.complete_round(public_config)?.ok_or(
            FederationError::DkgCommitmentRoundMismatch {
                reason: "complete DKG records did not produce a commitment round".to_string(),
            },
        )?;
        let derived_keyset_share = round.derive_member_keyset_share(
            public_config,
            &setup_state.reveals,
            local_member,
            &incoming_contributions,
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
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::LocalKeysetShare,
                base_progress,
                Some(commitment),
                Some(reveal),
                secret_shares,
                Some(derived_keyset_share),
                None,
            ));
        };
        if stored_keyset_share != &derived_keyset_share {
            return Err(FederationError::DkgTranscriptMismatch {
                reason: format!(
                    "stored local keyset share does not match ceremony {}",
                    participant_state.ceremony_id()
                ),
            });
        }

        let mut final_public_config = public_config.clone();
        final_public_config.keysets = vec![stored_keyset_share.keyset.clone()];
        final_public_config.setup_transcript = None;
        final_public_config.validate()?;
        let transcript_hash = FederationDkgTranscript::hash_for_config(
            FederationDkgSetupMethod::PedersenDkg,
            &final_public_config,
            std::slice::from_ref(&stored_keyset_share.public_ceremony),
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

        if matching_signatures.len() == member_count {
            FederationDkgFinalizedMemberConfig::from_records(
                public_config,
                private_config,
                std::slice::from_ref(stored_keyset_share),
                &matching_signatures.values().copied().collect::<Vec<_>>(),
            )?;
            return Ok(Self::plan_for_phase(
                FederationDkgDriverPhase::Complete,
                progress,
                Some(commitment),
                Some(reveal),
                secret_shares,
                None,
                Some(transcript_signature),
            ));
        }

        Ok(Self::plan_for_phase(
            FederationDkgDriverPhase::TranscriptSignature,
            progress,
            Some(commitment),
            Some(reveal),
            secret_shares,
            None,
            Some(transcript_signature),
        ))
    }

    fn plan_for_phase(
        phase: FederationDkgDriverPhase,
        progress: FederationDkgDriverProgress,
        commitment: Option<FederationDkgCommitmentRequest>,
        reveal: Option<FederationDkgRevealRequest>,
        secret_shares: BTreeMap<MemberId, FederationDkgSecretShareRequest>,
        derived_keyset_share: Option<FederationDkgMemberKeysetShare>,
        transcript_signature: Option<FederationDkgTranscriptSignatureRequest>,
    ) -> FederationDkgDriverPlan {
        FederationDkgDriverPlan {
            phase,
            progress,
            commitment,
            reveal,
            secret_shares,
            derived_keyset_share,
            transcript_signature,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::federation::{
        FederationDkgCeremonyId, FederationDkgKeysetSpec, MemberConfigPublic, ThresholdParams,
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
        let complete = records
            .plan(&public_config, private_config, participant_state)
            .expect("complete plan");
        assert_eq!(complete.phase, FederationDkgDriverPhase::Complete);
        assert_eq!(complete.progress.transcript_signature_count, 3);
        assert!(complete.commitment.is_some());
        assert!(complete.reveal.is_some());
        assert_eq!(complete.secret_shares.len(), 3);
        assert_eq!(complete.transcript_signature, Some(local_signature));
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
