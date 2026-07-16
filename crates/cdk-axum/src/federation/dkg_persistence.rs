//! DKG ceremony persistence owned independently from checkpoint and catch-up workflows.
//!
//! This module may validate and persist DKG/FROST ceremony records. It must not
//! select checkpoint targets, perform peer catch-up, or mutate readiness.

use std::sync::Arc;

use cdk::federation::{
    FederationConfig, FederationDkgActivationRequest, FederationDkgCeremonyId,
    FederationDkgFinalizedMemberConfig, FederationDkgMemberKeysetShare,
    FederationDkgParticipantCommitment, FederationDkgParticipantCommitmentReveal,
    FederationDkgSecretShareRequest, FederationDkgSetupState, FederationDkgTranscriptHash,
    FederationDkgTranscriptSignatureRequest, FederationError,
    FederationFrostDkgConfirmationRequest, FederationFrostDkgLocalState,
    FederationFrostDkgRound1Request, FederationFrostDkgRound2Record, FederationFrostRootRecord,
    FederationRestMessageKind, MemberId,
};

use super::node_runtime::{runtime_unavailable, FederationNodeRuntime};

impl FederationNodeRuntime {
    pub(crate) async fn persist_dkg_sequence(
        &self,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgCommitment, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_commitment(
        &self,
        commitment: &FederationDkgParticipantCommitment,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        let db = self.active_mint()?.localstore();
        let existing_commitments = db.get_federation_dkg_commitments().await?;
        if existing_commitments
            .iter()
            .any(|existing| existing == commitment)
        {
            commitment.validate_against_config(&public_config)?;
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, commitment.ceremony_id)
            .await?;
        setup_state.insert_commitment(&public_config, *commitment)?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_commitment(commitment).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgCommitment, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_reveal(
        &self,
        reveal: &FederationDkgParticipantCommitmentReveal,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        let db = self.active_mint()?.localstore();
        let existing_reveals = db.get_federation_dkg_reveals().await?;
        if existing_reveals.iter().any(|existing| existing == reveal) {
            let commitments = db.get_federation_dkg_commitments().await?;
            let commitment = commitments
                .iter()
                .find(|commitment| {
                    commitment.ceremony_id == reveal.ceremony_id
                        && commitment.member_id == reveal.member_id
                })
                .ok_or_else(|| {
                    cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                        member_id: reveal.member_id,
                        reason: "missing prior commitment".to_string(),
                    })
                })?;
            commitment.validate_reveal(reveal, &public_config)?;
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let mut setup_state = self
            .load_dkg_setup_state(&public_config, reveal.ceremony_id)
            .await?;
        setup_state.insert_reveal(&public_config, reveal.clone())?;

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_reveal(reveal).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgReveal, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_round1(
        &self,
        request: &FederationFrostDkgRound1Request,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_round1(request).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::FrostDkgRound1, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_round2(
        &self,
        record: &FederationFrostDkgRound2Record,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_round2(record).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::FrostDkgRound2, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_local_state(
        &self,
        state: &FederationFrostDkgLocalState,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.set_federation_frost_dkg_local_state(state).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_frost_root_record(
        &self,
        record: &FederationFrostRootRecord,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_root_record(record).await?;
        tx.commit().await?;
        Ok(())
    }

    pub(crate) async fn persist_frost_dkg_confirmation(
        &self,
        request: &FederationFrostDkgConfirmationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let db = self.active_mint()?.localstore();
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_frost_dkg_confirmation(request).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(
            FederationRestMessageKind::FrostDkgConfirmation,
            sender,
            sequence,
        )
        .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_secret_share_contribution(
        &self,
        contribution: &FederationDkgSecretShareRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        contribution.validate_sender_receiver(
            runtime_config.public_config(),
            sender,
            runtime_config.member_id(),
        )?;
        let public_config = runtime_config.public_config().clone();

        let db = mint.localstore();
        let existing_contributions = db.get_federation_dkg_secret_share_contributions().await?;
        if existing_contributions.iter().any(|existing| {
            existing.ceremony_id == contribution.ceremony_id
                && existing.sender == contribution.sender
                && existing.receiver == contribution.receiver
                && existing != contribution
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: format!(
                        "conflicting secret-share contribution for receiver {}",
                        contribution.receiver
                    ),
                },
            ));
        }
        if existing_contributions
            .iter()
            .any(|existing| existing == contribution)
        {
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let setup_state = self
            .load_dkg_setup_state(&public_config, contribution.ceremony_id)
            .await?;
        let ceremony_id = setup_state.ceremony_id.ok_or_else(|| {
            cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                member_id: contribution.sender,
                reason: "missing commitment ceremony before secret contribution".to_string(),
            })
        })?;
        if contribution.ceremony_id != ceremony_id {
            return Err(cdk::Error::Federation(
                FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: "secret contribution belongs to another ceremony".to_string(),
                },
            ));
        }
        let sender_reveal = setup_state
            .reveals
            .get(&contribution.sender)
            .cloned()
            .ok_or_else(|| {
                cdk::Error::Federation(FederationError::DkgCommitmentMismatch {
                    member_id: contribution.sender,
                    reason: "missing sender reveal before secret contribution".to_string(),
                })
            })?;
        let validation_config = public_config.clone();
        let validation_contribution = contribution.clone();
        crate::run_federation_dkg_cpu_task(
            "federation DKG secret-share verification task failed",
            move || {
                sender_reveal
                    .validate_secret_share_request(&validation_config, &validation_contribution)
            },
        )
        .await?;

        // Final keyset derivation belongs to the restartable driver after this durable write.
        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_secret_share_contribution(contribution)
            .await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgSecretShare, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_transcript_signature(
        &self,
        signature: &FederationDkgTranscriptSignatureRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        signature.validate_sender(&public_config, sender)?;
        let db = self.active_mint()?.localstore();
        let existing_signatures = db.get_federation_dkg_transcript_signatures().await?;
        if existing_signatures.iter().any(|existing| {
            existing.signer == signature.signer
                && existing.transcript_hash == signature.transcript_hash
                && existing != signature
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "conflicting transcript signature for member {} transcript {}",
                        signature.signer, signature.transcript_hash
                    ),
                },
            ));
        }
        if existing_signatures
            .iter()
            .any(|existing| existing == signature)
        {
            return self.persist_dkg_sequence(sender, sequence).await;
        }
        let expected_hash = self.expected_dkg_transcript_hash(&public_config).await?;
        if signature.transcript_hash != expected_hash {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "transcript signature hash {} does not match locally derived DKG transcript {}",
                        signature.transcript_hash,
                        expected_hash
                    ),
                },
            ));
        }

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_transcript_signature(signature)
            .await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(
            FederationRestMessageKind::DkgTranscriptSignature,
            sender,
            sequence,
        )
        .await;
        Ok(())
    }

    pub(crate) async fn persist_dkg_activation(
        &self,
        activation: &FederationDkgActivationRequest,
        sender: MemberId,
        sequence: u64,
    ) -> Result<(), cdk::Error> {
        let public_config = self.public_config()?;
        activation.validate_signature_sender(&public_config, sender)?;
        let db = self.active_mint()?.localstore();
        let existing_activations = db.get_federation_dkg_activations().await?;
        if existing_activations.iter().any(|existing| {
            existing.ceremony_id == activation.ceremony_id
                && existing.signer == activation.signer
                && existing != activation
        }) {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: format!(
                        "conflicting DKG activation for member {} ceremony {}",
                        activation.signer, activation.ceremony_id
                    ),
                },
            ));
        }
        if existing_activations
            .iter()
            .any(|existing| existing == activation)
        {
            return self.persist_dkg_sequence(sender, sequence).await;
        }

        let mut tx = db.begin_transaction().await?;
        tx.add_federation_dkg_activation(activation).await?;
        tx.set_federation_dkg_member_sequence(sender, sequence)
            .await?;
        tx.commit().await?;
        self.record_sequence(FederationRestMessageKind::DkgActivation, sender, sequence)
            .await;
        Ok(())
    }

    pub(crate) fn ready_dkg_finalized_member_config(
        &self,
    ) -> Option<Arc<FederationDkgFinalizedMemberConfig>> {
        self.components
            .dkg
            .member_admission
            .as_ref()?
            .finalized_config
            .get()
            .cloned()
    }

    pub(crate) async fn remember_dkg_finalized_member_config(
        &self,
        finalized: Arc<FederationDkgFinalizedMemberConfig>,
    ) -> Result<(), cdk::Error> {
        let Some(admission) = self.components.dkg.member_admission.as_ref() else {
            return Ok(());
        };
        let cached = admission
            .finalized_config
            .get_or_init(|| async { finalized.clone() })
            .await;
        if cached.as_ref() != finalized.as_ref() {
            return Err(cdk::Error::Federation(
                FederationError::DkgTranscriptMismatch {
                    reason: "cached finalized DKG config changed during one process".to_string(),
                },
            ));
        }
        Ok(())
    }

    async fn expected_dkg_transcript_hash(
        &self,
        setup_public_config: &FederationConfig,
    ) -> Result<FederationDkgTranscriptHash, cdk::Error> {
        let mint = self.active_mint()?;
        let runtime_config = mint
            .federation_config()
            .ok_or_else(|| runtime_unavailable("mint configuration"))?;
        let local_member = runtime_config.member_id();
        let keyset_shares = mint
            .localstore()
            .get_federation_dkg_member_keyset_shares()
            .await?;
        for share in &keyset_shares {
            if share.member_id != local_member {
                return Err(cdk::Error::Federation(FederationError::MemberIdMismatch {
                    expected: local_member,
                    found: share.member_id,
                }));
            }
        }
        Ok(
            FederationDkgMemberKeysetShare::transcript_hash_for_locally_derived_records(
                setup_public_config,
                &keyset_shares,
            )?,
        )
    }

    fn public_config(&self) -> Result<FederationConfig, cdk::Error> {
        self.active_mint()?
            .federation_config()
            .map(|config| config.public_config().clone())
            .ok_or_else(|| runtime_unavailable("mint configuration"))
    }

    async fn load_dkg_setup_state(
        &self,
        public_config: &FederationConfig,
        ceremony_id: FederationDkgCeremonyId,
    ) -> Result<FederationDkgSetupState, cdk::Error> {
        let db = self.active_mint()?.localstore();
        let commitments = db.get_federation_dkg_commitments().await?;
        let reveals = db.get_federation_dkg_reveals().await?;
        let mut setup_state = FederationDkgSetupState::new(public_config);
        for commitment in commitments
            .into_iter()
            .filter(|commitment| commitment.ceremony_id == ceremony_id)
        {
            setup_state.insert_commitment(public_config, commitment)?;
        }
        for reveal in reveals
            .into_iter()
            .filter(|reveal| reveal.ceremony_id == ceremony_id)
        {
            setup_state.insert_reveal(public_config, reveal)?;
        }
        Ok(setup_state)
    }
}
